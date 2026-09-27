/**
 * The OAuth flow state machine every provider screen shares (README §6.3):
 * the live flow and its pasted callback, the begin of a flow, the completion
 * of both grants, and the dead-flow resets. The screens parameterize it by
 * the action names the diagnoses specialize on, what a succeeded completion
 * does with the provider row, and the begin step's own prelude (the
 * creation page ensures its definition first).
 *
 * The flow's failures are the fragment's own: it states them on the signal
 * the flow card renders, beside the step and buttons that took the action —
 * never on the screen's read-level notice above the page (README §6.4). The
 * two channels share no write path, so one outcome cannot render twice.
 *
 * Disposal and flow identity are guarded here, in one home: a call answering
 * after the screen went away — or for a flow that is no longer the live one —
 * states nothing, navigates nowhere, and re-arms no poll, so the device
 * step's loop, which schedules only on the fragment's `"pending"`, stops by
 * itself (README §6.4).
 */
import { type Accessor, createSignal } from "solid-js";
import { nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ActionName, Diagnosis } from "/src/management-api/diagnosis";
import type {
    CompleteProviderOauthResponse,
    ProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { extractAuthorizationCode } from "/src/management-api/oauth-code";
import { type ManagementCallOutcome, performManagementCall } from "/src/management-api/outcome";
import { type ActionBegin, createBeginAction } from "./action-begin";
import { describeCallFailure } from "./failure";
import { createDisposalGuard, type ScreenNotice } from "./read";

/**
 * What one completion call's outcome left for the calling grant after the
 * shared settle ran: either a shared arm already acted — an aborted ask
 * states nothing, a gone flow state resets, a failed exchange burns the
 * flow, and a succeeded one installs the provider — or one of the two
 * grant-owned answers remains. The failed-but-not-gone diagnosis is stated
 * verbatim by the authorization-code caller and ridden out under the device
 * caller's poll budget; the pending answer is the device grant's to keep
 * polling, and one the authorization-code caller can never legally receive.
 */
type CompletionSettlement =
    | {
          readonly kind: "settled";
      }
    | {
          readonly kind: "failed";
          readonly diagnosis: Diagnosis;
      }
    | {
          readonly kind: "pending";
      };

/** The OAuth flow state machine one provider screen consumes. */
export type OauthFlow = {
    /** The live flow, when one is running. */
    readonly flow: Accessor<StartProviderOauthResponse | null>;
    /** The pasted callback text. */
    readonly code: Accessor<string>;
    readonly setCode: (value: string) => void;
    /** Whether a flow begin is in flight. */
    readonly isStarting: Accessor<boolean>;
    /** Whether an authorization-code completion is in flight. */
    readonly isCompleting: Accessor<boolean>;
    /** Whether either OAuth call is in flight. */
    readonly isBusy: Accessor<boolean>;
    /**
     * The flow's stated failure: a refused begin, a refused or failed
     * completion, a failed outcome poll, or a flow's expiry — stated by the
     * flow card beside the step and buttons that took the action, never on
     * the screen's read-level notice. Ending a flow clears it, as does a
     * poll answering again.
     */
    readonly failure: Accessor<ScreenNotice | null>;
    /**
     * The paste diagnosis beside the callback field: stated once a
     * completion was refused over a paste holding no readable code,
     * retracted once the paste holds one again, and gone with the flow.
     */
    readonly pasteError: Accessor<string | null>;
    /** Begins a flow on the gateway and installs what it answers. */
    readonly start: () => Promise<void>;
    /** Completes an authorization-code flow with the pasted callback. */
    readonly complete: () => Promise<void>;
    /**
     * One device-poll ask; the step panel schedules the next one on
     * `"pending"` and stops on `"done"`. A poll that fails without proving
     * the flow gone answers `"pending"` too — the flow stays live on the
     * gateway, its failure is stated in the flow card, and the loop re-arms
     * until `MAX_CONSECUTIVE_FAILED_POLLS` fail in a row. `"done"` ends the
     * loop only for a gone diagnosis, the flow's expiry, a stated terminal
     * outcome, an aborted ask, or disposal.
     */
    readonly completeDevice: () => Promise<"pending" | "done">;
    /** Discards the live flow, so a fresh one can start. */
    readonly reset: () => void;
};

/**
 * The inline diagnosis beside the paste field once a completion was refused
 * over a paste holding no readable code: one line naming the fault and its
 * fix, stated where the value was typed — the lazy treatment every field's
 * inline error gets, never a notice above the page.
 */
const UNREADABLE_PASTE_ERROR =
    "That paste holds neither a bare code nor a URL with a code — copy the whole address the login landed on, or just its code, and paste it here.";

/** The one recovery a spent or stalled flow leaves: begin a new one. */
const FRESH_FLOW_FIX = "Start a fresh flow and authorize it promptly.";

/** The recovery a failed-but-not-gone poll states: the gateway still holds
 * the flow pending, so the honest line is that this page keeps asking —
 * never the fresh-flow recovery, which would claim a live flow dead. */
const POLL_RETRY_FIX = "The flow stays live; this page keeps asking for the outcome.";

/**
 * How many consecutive failed outcome polls the device loop rides out before
 * it stops: one failure is a network blip, a sleeping laptop, or a restarting
 * gateway — the flow stays pending on the gateway and the loop re-arms. A run
 * this long means this page cannot reach the gateway, so it stops with the
 * fresh-flow recovery instead of asking forever. Kept beside the loop it
 * bounds: it is the polling loop's own retry budget, not a connection or
 * theme tunable.
 */
const MAX_CONSECUTIVE_FAILED_POLLS = 3;

/** The gateway's own verdict on a refused exchange, with the one recovery. */
function flowFailedNotice(failureMessage: string): ScreenNotice {
    return {
        tone: "bad",
        title: "The flow failed.",
        detail: failureMessage,
        fix: FRESH_FLOW_FIX,
    };
}

/**
 * The notice a completion whose flow state proved gone states: the flow's
 * own verdict, naming this flow as the subject and the spent state as the
 * cause where the raw row diagnosis names neither, with the gateway's
 * message kept verbatim as the detail and the one recovery a spent flow
 * leaves.
 */
function goneFlowNotice(diagnosis: Diagnosis): ScreenNotice {
    return {
        tone: "bad",
        title: "This authorization flow is no longer completable — its state expired or was already used.",
        detail: diagnosis.detail,
        fix: FRESH_FLOW_FIX,
    };
}

/**
 * The OAuth flow state machine, shared by the provider creation and
 * reauthorization screens (README §6.3).
 */
export function createOauthFlow(options: {
    /** The management API the flow's completion calls run against. */
    readonly api: ManagementApi;
    /** The action the flow-begin call's failures are diagnosed under. */
    readonly startAction: ActionName;
    /** The action the completion calls' failures are diagnosed under. */
    readonly completeAction: ActionName;
    /**
     * Begins a flow: the screen's own prelude — a refused one answers
     * `"blocked"` with its notice — followed by the gateway's flow-begin
     * call. The begin settles through the screens' shared begin action
     * against the flow card's failure signal.
     */
    readonly begin: () => Promise<ActionBegin<StartProviderOauthResponse>>;
    /** What a succeeded completion does with the provider row it answered. */
    readonly onSucceeded: (provider: ProviderResponse) => void;
    /**
     * Extra completion submittability beside a live authorization-code flow
     * and a readable paste — the creation page's prefix guard.
     */
    readonly isSubmittable?: Accessor<boolean> | undefined;
    /**
     * Runs beside the fragment's own reset when a completion's flow state
     * proves gone — unknown, expired, or already consumed.
     */
    readonly onGone?: (() => void) | undefined;
}): OauthFlow {
    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [isCompleting, setIsCompleting] = createSignal(false);
    // The flow's stated failure, rendered inside the flow card. The
    // fragment owns the signal outright — no screen channel is passed in —
    // so a flow failure structurally cannot reach the screen's read-level
    // notice, and the two can never render one outcome twice.
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);
    // Opened when a completion is refused over an unreadable paste; the
    // paste diagnosis then follows the field's current value until the flow
    // ends — the lazy gate every field's inline error opens behind.
    const [hasPasteRefusal, setHasPasteRefusal] = createSignal(false);

    // The consecutive failed outcome polls of the live device flow: any
    // answered poll resets it, and it bounds how long the loop rides out
    // transient failures before it stops (see MAX_CONSECUTIVE_FAILED_POLLS).
    let failedPolls = 0;

    // The one disposal guard, set when the consuming screen goes away: a call
    // answering afterwards must state nothing and re-arm nothing. The device
    // step's loop honors it through the "done" the polled asks return.
    const isDisposed = createDisposalGuard();

    // The flow's begin is the screens' shared begin action: the fragment's
    // own gates — no OAuth call in flight, no live flow — beside the settle
    // every begin answers, stated on the flow card's failure signal, with
    // the answered row installed as the live flow.
    const beginAction = createBeginAction({
        action: options.startAction,
        channel: setFailure,
        isDisposed,
        canRun: () => !isBusy() && flow() === null,
        perform: options.begin,
        onSucceeded: (started) => {
            setFlow(started);
        },
    });

    /** Whether either OAuth call is in flight. */
    const isBusy = (): boolean => beginAction.isInFlight() || isCompleting();

    /**
     * The paste diagnosis beside the callback field: standing only behind a
     * refused completion, and only while the paste still holds nothing
     * readable — a paste that reads retracts it without another ask.
     */
    const pasteError = (): string | null =>
        hasPasteRefusal() && extractAuthorizationCode(code()) === null
            ? UNREADABLE_PASTE_ERROR
            : null;

    /**
     * Discards the live flow: a consumed state is never retried — a failed
     * exchange burns the flow, and a terminal device outcome is consumed by
     * the one read that observed it (gateway README §7.2) — so only a fresh
     * flow can retry. The stated failure and the paste's refusal gate go
     * with the flow; a failure stated afresh after a discard rewrites the
     * cleared signal.
     */
    const discardFlow = (): void => {
        setFlow(null);
        setCode("");
        setHasPasteRefusal(false);
        setFailure(null);
        failedPolls = 0;
    };

    /**
     * The diagnosis of a failed completion, the raw material both verdicts
     * state: a gone state is reshaped into the flow's own notice, and any
     * other failure states the diagnosis verbatim. The device loop's
     * non-gone failures are stated separately, where the flow's liveness
     * dictates the recovery line instead.
     */
    const completionDiagnosis = (cause: unknown): Diagnosis =>
        describeCallFailure(cause, {
            action: options.completeAction,
        });

    /**
     * The state is unknown, expired, or already consumed: this flow is dead,
     * so it resets instead of being retried — and the notice is the flow's
     * own verdict, not the raw row diagnosis, which names neither this flow
     * nor the way forward.
     */
    const failGoneFlow = (diagnosis: Diagnosis): void => {
        discardFlow();
        options.onGone?.();
        setFailure(goneFlowNotice(diagnosis));
    };

    /** Ends a flow whose exchange the gateway answered as failed: the
     * burned flow resets, and the failure states the fresh-flow recovery. */
    const failExchange = (failureMessage: string): void => {
        discardFlow();
        setFailure(flowFailedNotice(failureMessage));
    };

    /**
     * The completion settle both grants share: it owns the four arms whose
     * behavior does not differ by grant — the aborted ask states nothing,
     * the gone flow state resets (`failGoneFlow`), the failed exchange burns
     * the flow (`failExchange`), and the succeeded one installs the provider
     * (`discardFlow` plus `onSucceeded`) — and hands the two grant-owned
     * answers back for the caller to translate into its own continuation.
     * Each arm acts at most once: the caller-owned arms touch no flow state
     * at all, so `discardFlow` runs exactly as often as before the settle
     * was shared.
     */
    const settleCompletionOutcome = (
        outcome: ManagementCallOutcome<CompleteProviderOauthResponse>,
    ): CompletionSettlement => {
        if (outcome.status === "aborted") {
            return {
                kind: "settled",
            };
        }

        if (outcome.status === "failed") {
            const diagnosis = completionDiagnosis(outcome.cause);

            if (diagnosis.kind === "gone") {
                failGoneFlow(diagnosis);

                return {
                    kind: "settled",
                };
            }

            return {
                kind: "failed",
                diagnosis,
            };
        }

        if (outcome.result.status === "failed") {
            failExchange(outcome.result.failure_message);

            return {
                kind: "settled",
            };
        }

        if (outcome.result.status === "succeeded") {
            discardFlow();
            options.onSucceeded(outcome.result.provider);

            return {
                kind: "settled",
            };
        }

        return {
            kind: "pending",
        };
    };

    const start = beginAction.run;

    /** Completes an authorization-code flow with the pasted callback. */
    const complete = async (): Promise<void> => {
        const startedFlow = flow();

        if (
            startedFlow === null ||
            startedFlow.grant !== "authorization_code" ||
            isBusy() ||
            options.isSubmittable?.() === false
        ) {
            return;
        }

        const pasted = extractAuthorizationCode(code());

        if (pasted === null) {
            // The paste is the field's own mistake: its diagnosis states
            // itself inline beside the field, and the flow survives.
            setHasPasteRefusal(true);
            return;
        }

        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            options.api.completeProviderOauth({
                grant: "authorization_code",
                state: startedFlow.state,
                code: pasted,
            }),
        );

        setIsCompleting(false);

        // The answer names the flow that was live when the ask left; a flow
        // replaced or abandoned since is not the answer's to act on.
        if (isDisposed() || flow()?.state !== startedFlow.state) {
            return;
        }

        const settlement = settleCompletionOutcome(outcome);

        if (settlement.kind === "failed") {
            setFailure({
                tone: "bad",
                title: settlement.diagnosis.title,
                detail: settlement.diagnosis.detail,
                fix: settlement.diagnosis.fix,
            });

            return;
        }

        if (settlement.kind === "pending") {
            // The ask carried the `authorization_code` grant, whose exchange the
            // gateway answers synchronously (gateway README §7.2); `pending` is
            // the device grant's answer alone, so receiving it here breaks the
            // contract and is fatal rather than silently ignored.
            throw new Error("The authorization-code completion answered pending.");
        }
    };

    /** One device-poll ask; the step panel schedules the next one on `"pending"`. */
    const completeDevice = async (): Promise<"pending" | "done"> => {
        const startedFlow = flow();

        if (isDisposed() || startedFlow === null || startedFlow.grant !== "device") {
            return "done";
        }

        // The flow's own expiry bounds the polling (gateway README §7.2):
        // past it the gateway can only answer 404, so the loop stops here and
        // says so instead of asking forever.
        if (nowMicros() >= startedFlow.expires_at) {
            discardFlow();
            setFailure({
                tone: "bad",
                title: "The flow expired.",
                detail: "The flow's expiry passed while this page waited for the outcome.",
                fix: FRESH_FLOW_FIX,
            });
            return "done";
        }

        const outcome = await performManagementCall(() =>
            options.api.completeProviderOauth({
                grant: "device",
                state: startedFlow.state,
            }),
        );

        // A poll answering after the screen went away — or for a flow that is
        // no longer the live one — acts on nothing, and the "done" it returns
        // is what stops the step panel's loop.
        if (isDisposed() || flow()?.state !== startedFlow.state) {
            return "done";
        }

        const settlement = settleCompletionOutcome(outcome);

        if (settlement.kind === "failed") {
            // The flow is still pending on the gateway: state the failure
            // without the fresh-flow recovery — which would claim a live flow
            // dead — and re-arm the loop, until too many polls fail in a row.
            // Then this page stops asking, stating the last failure with the
            // fresh-flow recovery as its one exit beside "Start over".
            failedPolls += 1;

            if (failedPolls >= MAX_CONSECUTIVE_FAILED_POLLS) {
                setFailure({
                    tone: "bad",
                    title: "The outcome poll kept failing, so this page stopped asking.",
                    detail: settlement.diagnosis.detail,
                    fix: FRESH_FLOW_FIX,
                });

                return "done";
            }

            setFailure({
                tone: "bad",
                title: "The last outcome poll failed.",
                detail: settlement.diagnosis.detail,
                fix: POLL_RETRY_FIX,
            });

            return "pending";
        }

        if (settlement.kind === "pending") {
            if (failedPolls > 0) {
                // A poll answered again: the stated failure is stale, and the
                // recovered loop clears it.
                setFailure(null);
            }

            failedPolls = 0;
            return "pending";
        }

        return "done";
    };

    return {
        flow,
        code,
        setCode,
        isStarting: beginAction.isInFlight,
        isCompleting,
        isBusy,
        failure,
        pasteError,
        start,
        complete,
        completeDevice,
        reset: discardFlow,
    };
}
