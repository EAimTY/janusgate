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
import { type Accessor, createSignal, onCleanup } from "solid-js";
import { nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ActionName } from "/src/management-api/diagnosis";
import type {
    ProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { extractAuthorizationCode } from "/src/management-api/oauth-code";
import { type ManagementCallOutcome, performManagementCall } from "/src/management-api/outcome";
import { describeCallFailure } from "./failure";
import { type ScreenNotice, screenFailureNotice } from "./read";

/**
 * What beginning a flow produced: the screen's own prelude refused and the
 * notice states why, or the gateway's flow-begin call ran and its outcome is
 * the fragment's to report.
 */
export type OauthFlowBegin =
    | {
          readonly status: "blocked";
          readonly notice: ScreenNotice;
      }
    | {
          readonly status: "called";
          readonly outcome: ManagementCallOutcome<StartProviderOauthResponse>;
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
     * `"blocked"` with its notice — followed by the gateway's flow-begin call.
     */
    readonly begin: () => Promise<OauthFlowBegin>;
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
    const [isStarting, setIsStarting] = createSignal(false);
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
    let isDisposed = false;
    onCleanup(() => {
        isDisposed = true;
    });

    /** Whether either OAuth call is in flight. */
    const isBusy = (): boolean => isStarting() || isCompleting();

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
     * The diagnosis of a failed completion, stated in the flow card. The
     * device loop's non-gone failures are stated separately, where the
     * flow's liveness dictates the recovery line instead.
     */
    const reportCompletionFailure = (cause: unknown): void => {
        setFailure(
            screenFailureNotice(cause, {
                action: options.completeAction,
            }),
        );
    };

    /** Whether a failed completion's flow state proves gone. */
    const isCompletionGone = (cause: unknown): boolean =>
        describeCallFailure(cause, {
            action: options.completeAction,
        }).kind === "gone";

    const start = async (): Promise<void> => {
        if (isBusy() || flow() !== null) {
            return;
        }

        setFailure(null);
        setIsStarting(true);

        const begun = await options.begin();

        setIsStarting(false);

        if (isDisposed) {
            return;
        }

        if (begun.status === "blocked") {
            setFailure(begun.notice);
            return;
        }

        if (begun.outcome.status === "aborted") {
            return;
        }

        if (begun.outcome.status === "failed") {
            setFailure(
                screenFailureNotice(begun.outcome.cause, {
                    action: options.startAction,
                }),
            );
            return;
        }

        setFlow(begun.outcome.result);
    };

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
        if (isDisposed || flow()?.state !== startedFlow.state) {
            return;
        }

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            if (isCompletionGone(outcome.cause)) {
                // The state is unknown, expired, or already consumed: this
                // flow is dead, so it resets instead of being retried.
                discardFlow();
                options.onGone?.();
            }

            reportCompletionFailure(outcome.cause);

            return;
        }

        if (outcome.result.status === "failed") {
            discardFlow();
            setFailure(flowFailedNotice(outcome.result.failure_message));
            return;
        }

        if (outcome.result.status === "succeeded") {
            discardFlow();
            options.onSucceeded(outcome.result.provider);
            return;
        }

        // The ask carried the `authorization_code` grant, whose exchange the
        // gateway answers synchronously (gateway README §7.2); `pending` is
        // the device grant's answer alone, so receiving it here breaks the
        // contract and is fatal rather than silently ignored.
        throw new Error("The authorization-code completion answered pending.");
    };

    /** One device-poll ask; the step panel schedules the next one on `"pending"`. */
    const completeDevice = async (): Promise<"pending" | "done"> => {
        const startedFlow = flow();

        if (isDisposed || startedFlow === null || startedFlow.grant !== "device") {
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
        if (isDisposed || flow()?.state !== startedFlow.state) {
            return "done";
        }

        if (outcome.status === "aborted") {
            return "done";
        }

        if (outcome.status === "failed") {
            if (isCompletionGone(outcome.cause)) {
                discardFlow();
                options.onGone?.();
                reportCompletionFailure(outcome.cause);
                return "done";
            }

            // The flow is still pending on the gateway: state the failure
            // without the fresh-flow recovery — which would claim a live flow
            // dead — and re-arm the loop, until too many polls fail in a row.
            // Then this page stops asking, stating the last failure with the
            // fresh-flow recovery as its one exit beside "Start over".
            const failure = screenFailureNotice(outcome.cause, {
                action: options.completeAction,
            });
            failedPolls += 1;

            if (failedPolls >= MAX_CONSECUTIVE_FAILED_POLLS) {
                setFailure({
                    tone: "bad",
                    title: "The outcome poll kept failing, so this page stopped asking.",
                    detail: failure.detail,
                    fix: FRESH_FLOW_FIX,
                });

                return "done";
            }

            setFailure({
                tone: "bad",
                title: "The last outcome poll failed.",
                detail: failure.detail,
                fix: POLL_RETRY_FIX,
            });

            return "pending";
        }

        if (outcome.result.status === "pending") {
            if (failedPolls > 0) {
                // A poll answered again: the stated failure is stale, and the
                // recovered loop clears it.
                setFailure(null);
            }

            failedPolls = 0;
            return "pending";
        }

        discardFlow();

        if (outcome.result.status === "failed") {
            setFailure(flowFailedNotice(outcome.result.failure_message));
            return "done";
        }

        if (outcome.result.status === "succeeded") {
            options.onSucceeded(outcome.result.provider);
        }

        return "done";
    };

    return {
        flow,
        code,
        setCode,
        isStarting,
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
