/**
 * The OAuth flow state machine every provider screen shares (README §6.3):
 * the live flow and its pasted callback, the begin of a flow, the completion
 * of both grants, and the dead-flow resets. The screens parameterize it by
 * the action names the diagnoses specialize on, the channel failures and
 * outcomes are stated on, what a succeeded completion does with the provider
 * row, and the begin step's own prelude (the creation page ensures its
 * definition first).
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
    /** Begins a flow on the gateway and installs what it answers. */
    readonly start: () => Promise<void>;
    /** Completes an authorization-code flow with the pasted callback. */
    readonly complete: () => Promise<void>;
    /**
     * One device-poll ask; the step panel schedules the next one on
     * `"pending"` and stops on `"done"`.
     */
    readonly completeDevice: () => Promise<"pending" | "done">;
    /** Discards the live flow, so a fresh one can start. */
    readonly reset: () => void;
};

/** The paste the completion cannot read, so the flow survives the mistake. */
const UNREADABLE_PASTE_NOTICE: ScreenNotice = {
    tone: "bad",
    title: "Paste the callback URL or the code.",
    detail: "That paste holds neither a bare code nor a URL with a code in it.",
    fix: "Copy the whole address the login landed on, or just its code, and paste it here.",
};

/** The one recovery a spent or stalled flow leaves: begin a new one. */
const FRESH_FLOW_FIX = "Start a fresh flow and authorize it promptly.";

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
    /** The channel the flow's failures and outcomes are stated on. */
    readonly channel: (notice: ScreenNotice | null) => void;
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
     * Discards the live flow: a consumed state is never retried — a failed
     * exchange burns the flow, and a terminal device outcome is consumed by
     * the one read that observed it (gateway README §7.2) — so only a fresh
     * flow can retry.
     */
    const discardFlow = (): void => {
        setFlow(null);
        setCode("");
    };

    /**
     * The diagnosis of a failed completion, stated on the screen's channel;
     * `fix` replaces the diagnosis's own recovery line where the flow's state
     * dictates the recovery instead.
     */
    const reportCompletionFailure = (cause: unknown, fix?: string): void => {
        const notice = screenFailureNotice(cause, {
            action: options.completeAction,
        });
        options.channel(
            fix === undefined
                ? notice
                : {
                      ...notice,
                      fix,
                  },
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

        options.channel(null);
        setIsStarting(true);

        const begun = await options.begin();

        setIsStarting(false);

        if (isDisposed) {
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
            options.channel(UNREADABLE_PASTE_NOTICE);
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
            options.channel(flowFailedNotice(outcome.result.failure_message));
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
            options.channel({
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

            // A transient failure leaves the flow live but has ended the loop,
            // so the panel's notice carries the same fresh-flow recovery a
            // refused exchange states — "Start over" is not the only exit.
            reportCompletionFailure(outcome.cause, FRESH_FLOW_FIX);

            return "done";
        }

        if (outcome.result.status === "pending") {
            return "pending";
        }

        discardFlow();

        if (outcome.result.status === "failed") {
            options.channel(flowFailedNotice(outcome.result.failure_message));
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
        start,
        complete,
        completeDevice,
        reset: discardFlow,
    };
}
