import type { Accessor } from "solid-js";
import { createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { ActionName } from "../management-api/diagnosis";
import { isAbortFailure } from "../management-api/errors";
import type { ManagementCallOutcome } from "../management-api/outcome";
import { performManagementCall } from "../management-api/outcome";
import { refreshRequest, reportRefresh } from "../refresh-state";
import type { FailureCall } from "./failure";
import { describeCallFailure } from "./failure";
import type { ScreenNotice } from "./notices";
import { createStandingNotice, screenFailureNotice } from "./notices";

/**
 * The failure of one read call, carrying the notice the screen states. The
 * screen harness catches it to install the diagnosis where the read's
 * failures are stated; anything else escaping a load is a bug and stays loud.
 */
export class ReadCallFailure extends Error {
    readonly notice: ScreenNotice;

    constructor(cause: unknown, call?: FailureCall) {
        super("A management read failed; the diagnosis is carried in `notice`.");
        this.name = "ReadCallFailure";
        this.notice = screenFailureNotice(cause, call);
    }
}

/** One management call inside a read cycle, run under the cycle's abort signal. */
export type ReadCall<TResult> = {
    /** The action the call attempts, which several failure diagnoses specialize on. */
    readonly action?: ActionName | undefined;
    readonly run: (signal: AbortSignal) => Promise<TResult>;
};

/** What an action's failure reporting accepts beside its diagnosis. */
type ReportActionOptions = {
    /**
     * Runs when the diagnosis says the row the call addressed is gone — the
     * screen's reload, usually, so the not-found state takes over instead of
     * a ghost form accepting re-saves forever (README §6.4).
     */
    readonly onGone?: (() => void) | undefined;
};

/**
 * Runs a read cycle's calls in parallel under one signal, answering every
 * result in call order. An aborted call ends the cycle ahead of any failure
 * (README §6.4); the first failed call in call order becomes the cycle's
 * `ReadCallFailure`, exactly as the call list is written. A failure that is
 * itself a nested read's `ReadCallFailure` — a composite read call runs an
 * inner `readAll` — is adopted whole rather than re-wrapped: its notice
 * already carries the leaf call's diagnosis, and adoption at every level lets
 * that diagnosis survive any nesting depth, where a re-wrap would bury it
 * under the generic "The call failed." rendering and lose the gateway's
 * verbatim message (README §6.4).
 */
export async function readAll<TResults extends readonly unknown[]>(
    signal: AbortSignal,
    ...calls: { [TIndex in keyof TResults]: ReadCall<TResults[TIndex]> }
): Promise<TResults> {
    const outcomes = await Promise.all(
        calls.map((entry) => performManagementCall(() => entry.run(signal))),
    );

    for (const outcome of outcomes) {
        if (outcome.status === "aborted") {
            throw new DOMException("This read cycle was aborted.", "AbortError");
        }
    }

    const results: unknown[] = [];

    for (const [index, outcome] of outcomes.entries()) {
        if (outcome.status === "failed") {
            if (outcome.cause instanceof ReadCallFailure) {
                throw outcome.cause;
            }

            throw new ReadCallFailure(outcome.cause, {
                action: calls[index]?.action,
            });
        }

        results.push(outcome.status === "ok" ? outcome.result : undefined);
    }

    // Every outcome is known `ok` at this point, so the collected results are
    // exactly the tuple the mapped call list promised; the double assertion
    // only restores that tuple's shape for the caller.
    return results as unknown as TResults;
}

/** Runs one management call as a read cycle of its own. */
export async function readOne<TResult>(
    signal: AbortSignal,
    call: ReadCall<TResult>,
): Promise<TResult> {
    const [result] = await readAll(signal, call);
    return result;
}

/**
 * Whether the skeleton window is open: a read is in flight, no read has ever
 * succeeded, and nothing stands in the notice. The notice clause is
 * load-bearing — it keeps a standing failure visible while its re-read runs
 * (on an `isNoticeKeptOnSuccess` screen a standing notice may also be an
 * action's reported outcome), so the skeleton never replaces it. The
 * harness's `isFirstRead` accessor is this one predicate over its own
 * signals; a screen outside the harness — the usage query screen, whose read
 * cycle is also a form submission — calls it with its own.
 */
export function isSkeletonWindowOpen(
    isReading: boolean,
    hasRead: boolean,
    notice: ScreenNotice | null,
): boolean {
    return isReading && !hasRead && notice === null;
}

/** One mounted screen's read cycle: what every screen shares around its reads. */
export type ScreenRead = {
    /** Whether any read has ever succeeded on this screen. */
    readonly hasRead: Accessor<boolean>;
    /** Whether a read cycle is in flight — including the first one. */
    readonly isReading: Accessor<boolean>;
    /** Whether the skeleton window is open — `isSkeletonWindowOpen` over this harness's signals. */
    readonly isFirstRead: Accessor<boolean>;
    /** Whether the screen has gone away: actions answering later must stop. */
    readonly isDisposed: Accessor<boolean>;
    /** The standing notice: the last read failure, or the last action's outcome. */
    readonly notice: Accessor<ScreenNotice | null>;
    /** States an action's outcome where the read failures are stated. */
    readonly showNotice: (notice: ScreenNotice | null) => void;
    /**
     * States a finished action call's outcome where the read failures are
     * stated, and answers whether the call succeeded, so the caller keeps
     * only its success half. An aborted call states nothing; a failed one is
     * diagnosed into the standing notice — a `gone` diagnosis first runs
     * `onGone`.
     */
    readonly reportAction: <TResult>(
        outcome: ManagementCallOutcome<TResult>,
        call: FailureCall,
        options?: ReportActionOptions,
    ) => outcome is Extract<
        ManagementCallOutcome<TResult>,
        {
            readonly status: "ok";
        }
    >;
    /** Runs one read cycle now — a successful action's explicit reload. */
    readonly reload: () => void;
};

/**
 * The screen read harness (README §6.3): one read per mount, repeated when
 * *refresh* is pressed, cancelled when the screen goes away, with the first
 * failure diagnosed where failures are stated. A route-identity move is a
 * keyed remount (README §6.3) — the fresh mount runs its own first read — so
 * this harness follows the refresh pulse alone. The screen owns its data
 * signals and the `load` body — what this screen loads and where each answer
 * goes — while the harness owns the signals every screen redeclared, the
 * abort lifecycle, the refresh subscription, and the diagnosis mapping.
 *
 * A failed cycle replaces the standing notice with its diagnosis. A
 * successful cycle clears it — except where `isNoticeKeptOnSuccess` holds,
 * for screens whose notice is also an action's reported outcome. The
 * exception is an origin rule, not a blanket (README §6.3): the policy keeps
 * only what an action stated, so a save's outcome survives the reload the
 * save itself triggered, while a read-failure notice diagnoses the last read
 * cycle and is cleared by the first read that succeeds, on kept screens too
 * — the Read-again recovery lands on the row, not under its failure.
 */
export function createScreenRead(options: {
    /** The screen's reads, run under the cycle's signal; throws `ReadCallFailure` to fail the cycle. */
    readonly load: (signal: AbortSignal) => Promise<void>;
    /**
     * Whether a successful read keeps a notice an action stated (a read
     * failure is cleared by the read that succeeds regardless).
     */
    readonly isNoticeKeptOnSuccess?: boolean;
}): ScreenRead {
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const standing = createStandingNotice();
    const [isDisposed, setIsDisposed] = createSignal(false);

    let controller: AbortController | null = null;

    const run = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        try {
            await options.load(call.signal);
        } catch (error) {
            if (isAbortFailure(error) || call.signal.aborted) {
                // Superseded or disposed: an aborted read installs nothing —
                // not even its diagnosis, which belongs to a cycle the screen
                // has already replaced.
                return;
            }

            if (error instanceof ReadCallFailure) {
                setIsReading(false);
                standing.state("read", error.notice);
                return;
            }

            throw error;
        }

        if (call.signal.aborted) {
            // Superseded after the reads resolved but before this
            // continuation ran: the cycle is dead either way, and the newer
            // cycle owns the screen — nothing installs, not even `hasRead`.
            return;
        }

        setIsReading(false);

        // The two-origin rule (README §6.3): the keep policy exists so a
        // save's reported outcome survives the reload the save itself
        // triggered, so it shields only a notice an action stated. A
        // read-failure notice diagnoses the last read cycle — this one,
        // which just succeeded — and is stale on any screen, kept or not,
        // so a successful read clears it. Neither origin reaches across:
        // reads never clear an action's notice on a kept screen, and
        // actions never clear a read's except by stating their own or
        // dismissing explicitly (`showNotice(null)`).
        const isNoticeShielded =
            options.isNoticeKeptOnSuccess === true && standing.origin() === "action";

        if (!isNoticeShielded) {
            standing.clear();
        }

        setHasRead(true);
        reportRefresh();
    };

    createEffect(
        () => refreshRequest(),
        () => {
            void run();

            return () => {
                controller?.abort();
            };
        },
    );

    onCleanup(() => {
        setIsDisposed(true);
        controller?.abort();
    });

    // Computed once, so every screen states the skeleton's window through the
    // same one definition instead of re-spelling the three-clause gate.
    const isFirstRead = createMemo(() =>
        isSkeletonWindowOpen(isReading(), hasRead(), standing.notice()),
    );

    return {
        hasRead,
        isReading,
        isFirstRead,
        isDisposed,
        notice: standing.notice,
        showNotice: (stated) => {
            if (stated === null) {
                standing.clear();
                return;
            }

            standing.state("action", stated);
        },
        reportAction: <TResult>(
            outcome: ManagementCallOutcome<TResult>,
            call: FailureCall,
            options?: ReportActionOptions,
        ): outcome is Extract<
            ManagementCallOutcome<TResult>,
            {
                readonly status: "ok";
            }
        > => {
            if (outcome.status === "aborted") {
                return false;
            }

            if (outcome.status === "failed") {
                if (describeCallFailure(outcome.cause, call).kind === "gone") {
                    options?.onGone?.();
                }

                standing.state("action", screenFailureNotice(outcome.cause, call));

                return false;
            }

            return true;
        },
        reload: () => {
            if (!isDisposed()) {
                void run();
            }
        },
    };
}
