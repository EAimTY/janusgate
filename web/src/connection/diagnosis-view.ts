import { type Accessor, createEffect, createSignal } from "solid-js";
import { appConfig } from "/src/config";
import type { ConnectionState } from "/src/connection/state";
import { type Diagnosis, diagnoseFailure, probeReachability } from "/src/management-api/diagnosis";
import { deadlineController } from "./deadline";

/** The connection state narrowed to its unconnected landing — `null` while verifying or connected. */
type UnconnectedLanding = Extract<
    ConnectionState,
    {
        status: "unconnected";
    }
>;

/**
 * The setup screen's reactive diagnosis machine, beside the connection
 * machine it reads (README §6.4). Every unconnected landing is examined: a
 * landing without a failure — a cancellation's above all — is control flow,
 * never a write-up, the landing's address is handed back for the form to
 * seed, and a "no answer" verdict is refined by one reachability probe —
 * first telling "nothing is listening" from "the browser blocked the
 * response" apart. A re-run of the effect (a newer refusal) or the owner's
 * disposal aborts the refinement, so superseded diagnoses never leave an
 * in-flight probe behind, and the probe carries the connection deadline as
 * the backstop that keeps a blackhole address from leaving the refinement
 * pending minutes after the screen gave up.
 */
export function createConnectionDiagnosis(props: {
    /** The connection state the machine reads. */
    readonly state: Accessor<ConnectionState>;
    /**
     * The address the setup form's field holds: the fallback a failure
     * without an address of its own attempts. Read once per landing, never
     * tracked — the machine reacts to landings, not to typing.
     */
    readonly fieldBaseUrl: Accessor<string>;
    /**
     * Invoked on every landing with the address it carries for the form
     * field — a refusal's attempted address or a forget's served address —
     * or `null` when it carries none.
     */
    readonly onSeed: (baseUrl: string | null) => void;
}): Accessor<Diagnosis | null> {
    const [diagnosis, setDiagnosis] = createSignal<Diagnosis | null>(null);

    createEffect(
        () => {
            const current = props.state();
            return current.status === "unconnected" ? current : null;
        },
        (unconnected: UnconnectedLanding | null) => {
            // The refusal seeds from the address its failure carried — a
            // mid-session 401's, so the field reopens with it and a later
            // edit of the field cannot misattribute the failure — and a
            // deliberate forget seeds from the address the connection
            // served, so the standard token rotation (disconnect, forget,
            // paste, connect) never demands an origin retype. The token is
            // never carried or seeded: it is a bearer secret only the user
            // can re-enter.
            props.onSeed(unconnected?.failure?.baseUrl ?? unconnected?.forgottenBaseUrl ?? null);

            const failure = unconnected?.failure ?? null;

            if (failure === null) {
                // Connected, verifying, pristine, or freshly forgotten: a
                // forget diagnoses nothing, because nothing was refused.
                setDiagnosis(null);
                return;
            }

            const attempted = failure.baseUrl ?? props.fieldBaseUrl();
            const context = {
                baseUrl: attempted,
                pageOrigin: window.location.origin,
                action: "connect" as const,
            };
            const initial = diagnoseFailure(failure.cause ?? failure.message, context);

            setDiagnosis(initial);

            if (initial.kind !== "no-answer") {
                return;
            }

            // The probe separates "nothing is listening" from "the browser blocked
            // the response"; it is only worth a request while the answer is open.
            // This diagnosis owns its probe: a re-run of this effect (a newer
            // refusal) or the screen going away aborts it, so superseded
            // diagnoses never leave an in-flight probe behind (README §6.4).
            // The deadline stays on as the secondary backstop, so a
            // blackhole address cannot leave the refinement pending minutes
            // after the screen gave up. The flag still discards an answer
            // that outlives this diagnosis, the aborted probe's "unknown"
            // included.
            let isCurrent = true;
            const refinementController = deadlineController(appConfig.connectionProbeTimeoutMs);

            void probeReachability(attempted, refinementController.signal).then((reachability) => {
                if (!isCurrent) {
                    return;
                }

                setDiagnosis(
                    diagnoseFailure(failure.cause ?? failure.message, {
                        ...context,
                        reachability,
                    }),
                );
            });

            return () => {
                isCurrent = false;
                refinementController.abort();
            };
        },
    );

    return diagnosis;
}
